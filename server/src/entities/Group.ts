import { Entity, JoinTable, OneToMany, PrimaryColumn } from "typeorm";
import { User } from "./User";

@Entity()
export class Group {
  @PrimaryColumn()
  gid: number;

  @OneToMany(() => User, (user) => user.group)
  @JoinTable()
  users: User[];
}