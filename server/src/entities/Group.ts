import { Entity, JoinTable, ManyToMany, OneToMany, PrimaryColumn } from "typeorm";
import { User } from "./User";

@Entity()
export class Group {
  @PrimaryColumn()
  gid: number;

  @ManyToMany(() => User, (user) => user.groups)
  @JoinTable()
  users: User[];
}