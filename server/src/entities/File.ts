import { Entity, Column, PrimaryColumn, ManyToOne, JoinColumn } from "typeorm";
import { User } from "./User";
import { Group } from "./Group";

@Entity()
export class File {
  @PrimaryColumn()
  path: string;

  @ManyToOne(() => User, (user) => user.files)
  @JoinColumn({ name: "owner", referencedColumnName: "uid" })
  owner: User;

  @Column()
  type: number; // 0 = file, 1 = directory, 2 = symlink, etc.

  @Column()
  permissions: number;

  @ManyToOne(() => Group, (group) => group.gid, { nullable: true })
  @JoinColumn({ name: "group" })
  group: Group;

}